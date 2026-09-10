//! Bounded collection navigation with stable selection across refresh and reflow.
//!
//! Measure rows with `Context::paginate_rows` (or its trailing-controls variant),
//! then give those pages and the corresponding stable record IDs to `reflow`.
//! The app retains its own records; this type only owns navigation state.

use std::collections::BTreeSet;

pub const MAX_COLLECTION_ITEMS: usize = 2048;
pub const MAX_RECORD_ID_BYTES: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CollectionError {
    TooManyItems,
    InvalidId,
    DuplicateId,
    InvalidPages,
}

impl std::fmt::Display for CollectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::TooManyItems => "The collection has too many items for one load. Request another page from the provider.",
            Self::InvalidId => "Each collection item needs a nonempty stable ID of at most 256 bytes.",
            Self::DuplicateId => "Collection IDs must be unique.",
            Self::InvalidPages => "Measured pages must include each item exactly once.",
        })
    }
}
impl std::error::Error for CollectionError {}

/// A record anchor survives changes in row height, order and page capacity.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CollectionPosition {
    pub anchor: Option<String>,
    pub selected: Option<String>,
    /// Fallback when the anchor was removed from a refreshed collection.
    pub page: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PagedCollection {
    ids: Vec<String>,
    pages: Vec<Vec<usize>>,
    position: CollectionPosition,
}

impl PagedCollection {
    /// Restore a previously saved navigation position. `reflow` resolves it.
    #[must_use]
    pub fn at(position: CollectionPosition) -> Self {
        Self {
            position,
            ..Self::default()
        }
    }

    /// Replace collection IDs and measured pages without losing the visible record.
    /// Invalid input leaves the previous state unchanged.
    ///
    /// # Errors
    /// Refuses oversized/duplicate IDs and incomplete or repeated page indexes.
    pub fn reflow(
        &mut self,
        ids: Vec<String>,
        pages: Vec<Vec<usize>>,
    ) -> Result<(), CollectionError> {
        if ids.len() > MAX_COLLECTION_ITEMS {
            return Err(CollectionError::TooManyItems);
        }
        let mut unique = BTreeSet::new();
        for id in &ids {
            if id.is_empty() || id.len() > MAX_RECORD_ID_BYTES {
                return Err(CollectionError::InvalidId);
            }
            if !unique.insert(id.as_str()) {
                return Err(CollectionError::DuplicateId);
            }
        }
        if pages.len() > ids.len() {
            return Err(CollectionError::InvalidPages);
        }
        let mut seen = vec![false; ids.len()];
        for page in &pages {
            if page.is_empty() || page.len() > ids.len() {
                return Err(CollectionError::InvalidPages);
            }
            for &index in page {
                let Some(visited) = seen.get_mut(index) else {
                    return Err(CollectionError::InvalidPages);
                };
                if *visited {
                    return Err(CollectionError::InvalidPages);
                }
                *visited = true;
            }
        }
        if seen.contains(&false) {
            return Err(CollectionError::InvalidPages);
        }
        let anchor = self
            .position
            .anchor
            .as_ref()
            .and_then(|id| ids.iter().position(|item| item == id));
        self.position.page = anchor
            .and_then(|index| pages.iter().position(|page| page.contains(&index)))
            .unwrap_or(self.position.page)
            .min(pages.len().saturating_sub(1));
        if self
            .position
            .selected
            .as_ref()
            .is_some_and(|selected| !unique.contains(selected.as_str()))
        {
            self.position.selected = None;
        }
        self.ids = ids;
        self.pages = pages;
        self.refresh_anchor();
        Ok(())
    }

    #[must_use]
    pub fn visible(&self) -> &[usize] {
        self.pages
            .get(self.position.page)
            .map_or(&[], Vec::as_slice)
    }

    #[must_use]
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    #[must_use]
    pub fn position(&self) -> &CollectionPosition {
        &self.position
    }

    /// Record selection before opening details so returning restores its page.
    pub fn select(&mut self, id: &str) -> bool {
        let Some(index) = self.ids.iter().position(|item| item == id) else {
            return false;
        };
        let Some(page) = self.pages.iter().position(|page| page.contains(&index)) else {
            return false;
        };
        self.position.page = page;
        self.position.selected = Some(id.to_owned());
        self.position.anchor = Some(id.to_owned());
        true
    }

    /// Move to a page, clamped to the collection. Returns whether it changed.
    pub fn go_to(&mut self, page: usize) -> bool {
        let next = page.min(self.pages.len().saturating_sub(1));
        if next == self.position.page {
            return false;
        }
        self.position.page = next;
        self.refresh_anchor();
        true
    }

    pub fn next_page(&mut self) -> bool {
        self.go_to(self.position.page.saturating_add(1))
    }
    pub fn previous_page(&mut self) -> bool {
        self.go_to(self.position.page.saturating_sub(1))
    }

    fn refresh_anchor(&mut self) {
        let selected_visible = self.position.selected.as_ref().is_some_and(|selected| {
            self.visible()
                .iter()
                .any(|&index| self.ids[index] == *selected)
        });
        self.position.anchor = if selected_visible {
            self.position.selected.clone()
        } else {
            self.visible().first().map(|&index| self.ids[index].clone())
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ids(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn selection_survives_refresh_and_larger_text_reflow() {
        let mut list = PagedCollection::default();
        list.reflow(ids(&["a", "b", "c", "d"]), vec![vec![0, 1], vec![2, 3]])
            .unwrap();
        assert!(list.select("c"));
        list.reflow(
            ids(&["new", "d", "c", "b", "a"]),
            vec![vec![0], vec![1], vec![2], vec![3], vec![4]],
        )
        .unwrap();
        assert_eq!(list.position().page, 2);
        assert_eq!(list.position().selected.as_deref(), Some("c"));
        assert_eq!(list.visible(), [2]);
    }

    #[test]
    fn deletion_and_empty_refresh_leave_a_reachable_page() {
        let mut list = PagedCollection::default();
        list.reflow(ids(&["a", "b", "c"]), vec![vec![0], vec![1], vec![2]])
            .unwrap();
        list.select("c");
        list.reflow(ids(&["a", "b"]), vec![vec![0], vec![1]])
            .unwrap();
        assert_eq!(list.position().page, 1);
        assert_eq!(list.position().selected, None);
        list.reflow(vec![], vec![]).unwrap();
        assert_eq!(list.position(), &CollectionPosition::default());
        assert!(!list.next_page());
        assert!(!list.previous_page());
    }

    #[test]
    fn malformed_reflow_does_not_replace_the_working_collection() {
        let mut list = PagedCollection::default();
        list.reflow(ids(&["a", "b"]), vec![vec![0, 1]]).unwrap();
        let before = list.clone();
        for (keys, pages) in [
            (ids(&["a", "a"]), vec![vec![0, 1]]),
            (ids(&["a", "b"]), vec![vec![0, 0]]),
            (ids(&["a", "b"]), vec![vec![0]]),
            (ids(&["a", "b"]), vec![vec![0, 2]]),
            (ids(&["a", "b"]), vec![vec![], vec![0, 1]]),
        ] {
            assert!(list.reflow(keys, pages).is_err());
            assert_eq!(list, before);
        }
    }

    #[test]
    fn a_saved_record_anchor_beats_an_obsolete_page_number() {
        let mut list = PagedCollection::at(CollectionPosition {
            anchor: Some("b".into()),
            selected: Some("b".into()),
            page: 99,
        });
        list.reflow(ids(&["a", "b", "c"]), vec![vec![0, 1], vec![2]])
            .unwrap();
        assert_eq!(list.visible(), [0, 1]);
        assert!(list.next_page());
        assert_eq!(list.position().anchor.as_deref(), Some("c"));
        assert!(!list.next_page());
    }
}
