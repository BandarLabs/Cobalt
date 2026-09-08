//! Per-volume reading memory and a lazy, two-page decode cache with opt-in color.
//! Persistence is acknowledged by the caller's store; encoding is not saving.

use crate::{
    viewport::{Fit, Viewport},
    Comic, ComicError,
};
use kobo_image::Picture;
use kobo_json::{ObjectBuilder, Value};
use std::collections::VecDeque;

pub const MAX_MEMORY_BYTES: usize = 8 * 1024;
pub const MAX_CACHED_PAGES: usize = 2;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Memory {
    pub page: usize,
    pub right_to_left: bool,
    pub spreads: bool,
    pub viewport: Viewport,
}
impl Memory {
    #[must_use]
    pub fn new(comic: &Comic) -> Self {
        Self {
            page: 0,
            right_to_left: comic.metadata.right_to_left.unwrap_or(false),
            spreads: false,
            viewport: Viewport::default(),
        }
    }
    /// Versioned reading position using a filename anchor. Page indexes only
    /// serve as a fallback after an updated volume removes the old page.
    ///
    /// # Errors
    /// Refuses invalid page indexes or excessive anchors.
    pub fn encode(&self, comic: &Comic) -> Result<Vec<u8>, String> {
        let anchor = comic
            .pages
            .get(self.page)
            .ok_or("Reading position is outside this comic.")?;
        let (x, y) = self.viewport.position();
        let bytes = ObjectBuilder::new()
            .set("version", 1_u32)
            .set(
                "page",
                u32::try_from(self.page).map_err(|_| "Reading position is too large.")?,
            )
            .set("anchor", anchor.as_str())
            .set("rtl", self.right_to_left)
            .set("spreads", self.spreads)
            .set(
                "fit",
                match self.viewport.fit {
                    Fit::Page => "page",
                    Fit::Width => "width",
                },
            )
            .set("zoom", u32::from(self.viewport.zoom()))
            .set("x", u32::from(x))
            .set("y", u32::from(y))
            .build()
            .to_json()
            .into_bytes();
        if bytes.len() > MAX_MEMORY_BYTES {
            return Err("Reading position exceeds the storage limit.".into());
        }
        Ok(bytes)
    }

    /// Restores the shared schema or migrates Panels' old decimal page index.
    ///
    /// # Errors
    /// Distinguishes unreadable or newer records from an expected missing one.
    pub fn restore(bytes: Option<&[u8]>, comic: &Comic) -> Result<Self, String> {
        let Some(bytes) = bytes else {
            return Ok(Self::new(comic));
        };
        if bytes.len() > MAX_MEMORY_BYTES {
            return Err("Saved reading position is too large.".into());
        }
        let source =
            std::str::from_utf8(bytes).map_err(|_| "Saved reading position could not be read.")?;
        if let Ok(page) = source.parse::<usize>() {
            return Ok(Self {
                page: page.min(comic.pages.len().saturating_sub(1)),
                ..Self::new(comic)
            });
        }
        let value =
            kobo_json::parse(source).map_err(|_| "Saved reading position could not be read.")?;
        let num = |name| {
            value
                .get(name)
                .and_then(Value::as_i64)
                .and_then(|n| u32::try_from(n).ok())
                .ok_or("Saved reading position has an invalid number.")
        };
        if num("version")? != 1 {
            return Err("Update the app to restore this reading position.".into());
        }
        let text = |name| {
            value
                .get(name)
                .and_then(Value::as_str)
                .ok_or("Saved reading position is incomplete.")
        };
        let boolean = |name| {
            value
                .get(name)
                .and_then(Value::as_bool)
                .ok_or("Saved reading position is incomplete.")
        };
        let fit = match text("fit")? {
            "page" => Fit::Page,
            "width" => Fit::Width,
            _ => return Err("Saved page fit is not supported.".into()),
        };
        let (zoom, x, y) = (num("zoom")?, num("x")?, num("y")?);
        if !(100..=400).contains(&zoom) || x > 10_000 || y > 10_000 {
            return Err("Saved zoom or pan is outside the supported range.".into());
        }
        let fallback = num("page")? as usize;
        let page = comic
            .pages
            .iter()
            .position(|name| name == text("anchor").unwrap_or_default())
            .unwrap_or(fallback)
            .min(comic.pages.len().saturating_sub(1));
        // Require the anchor even when the index still exists.
        text("anchor")?;
        Ok(Self {
            page,
            right_to_left: boolean("rtl")?,
            spreads: boolean("spreads")?,
            viewport: Viewport::new(
                fit,
                u16::try_from(zoom).map_err(|_| "Invalid zoom.")?,
                u16::try_from(x).map_err(|_| "Invalid pan.")?,
                u16::try_from(y).map_err(|_| "Invalid pan.")?,
            ),
        })
    }
}

#[derive(Debug)]
pub struct Reader {
    bytes: Vec<u8>,
    comic: Comic,
    memory: Memory,
    cache: VecDeque<(usize, Picture)>,
    colour: bool,
}
impl Reader {
    /// Opens one bounded archive without decoding any page.
    ///
    /// # Errors
    /// Returns the archive admission error from [`crate::inspect`].
    pub fn open(bytes: Vec<u8>) -> Result<Self, ComicError> {
        let comic = crate::inspect(&bytes)?;
        let memory = Memory::new(&comic);
        Ok(Self {
            bytes,
            comic,
            memory,
            cache: VecDeque::new(),
            colour: false,
        })
    }
    #[must_use]
    pub const fn comic(&self) -> &Comic {
        &self.comic
    }
    #[must_use]
    pub const fn memory(&self) -> &Memory {
        &self.memory
    }
    pub fn memory_mut(&mut self) -> &mut Memory {
        &mut self.memory
    }
    pub fn jump(&mut self, page: usize) -> bool {
        if page >= self.comic.pages.len() || page == self.memory.page {
            return false;
        }
        self.memory.page = page;
        let (x, _) = self.memory.viewport.position();
        self.memory.viewport =
            Viewport::new(self.memory.viewport.fit, self.memory.viewport.zoom(), x, 0);
        true
    }
    /// Restores this volume's memory without dropping the current position on
    /// a malformed saved record.
    ///
    /// # Errors
    /// Returns a migration or schema error, leaving memory unchanged.
    pub fn restore(&mut self, bytes: Option<&[u8]>) -> Result<(), String> {
        self.memory = Memory::restore(bytes, &self.comic)?;
        Ok(())
    }
    #[must_use]
    pub fn visible_pages(&self, target: (u32, u32)) -> Vec<usize> {
        let page = self
            .memory
            .page
            .min(self.comic.pages.len().saturating_sub(1));
        if self.memory.spreads
            && target.0 > target.1
            && page != self.comic.metadata.cover.unwrap_or(0)
            && page + 1 != self.comic.metadata.cover.unwrap_or(0)
            && page + 1 < self.comic.pages.len()
        {
            vec![page, page + 1]
        } else {
            vec![page]
        }
    }
    /// Moves in narrative order; UI side zones decide how RTL maps to this.
    pub fn turn(&mut self, forward: bool, target: (u32, u32)) -> bool {
        let page = self.memory.page;
        let next = if forward {
            page.saturating_add(self.visible_pages(target).len())
        } else if self.memory.spreads && target.0 > target.1 && page >= 2 {
            let cover = self.comic.metadata.cover.unwrap_or(0);
            if cover == page - 1 || cover == page - 2 {
                page - 1
            } else {
                page - 2
            }
        } else {
            page.saturating_sub(1)
        };
        self.jump(next)
    }

    /// Enable only after the host has reported color support. Cached grayscale
    /// pages must be decoded again, without changing the reading position.
    pub fn set_colour(&mut self, enabled: bool) {
        if self.colour != enabled {
            self.colour = enabled;
            self.cache.clear();
        }
    }

    fn decoded(&mut self, index: usize) -> Result<&Picture, ComicError> {
        if let Some(at) = self.cache.iter().position(|(page, _)| *page == index) {
            let entry = self.cache.remove(at).expect("known cache entry");
            self.cache.push_front(entry);
        } else {
            while self.cache.len() >= MAX_CACHED_PAGES {
                self.cache.pop_back();
            }
            let picture = crate::page_with_colour(&self.bytes, &self.comic, index, self.colour)?;
            let bytes = |p: &Picture| p.grey().len() + p.colour().map_or(0, <[u8]>::len);
            // At most two pages and one maximum-sized RGB+grey decode's bytes.
            let limit = usize::try_from(kobo_image::MAX_PIXELS).unwrap_or(usize::MAX / 4) * 4;
            while !self.cache.is_empty()
                && self.cache.iter().map(|(_, p)| bytes(p)).sum::<usize>() + bytes(&picture) > limit
            {
                self.cache.pop_back();
            }
            self.cache.push_front((index, picture));
        }
        Ok(&self.cache.front().expect("decoded page").1)
    }
    /// Renders the visible page(s), decoding at most one new image at a time.
    ///
    /// # Errors
    /// Returns actionable archive, decode or viewport errors.
    pub fn render(&mut self, target: (u32, u32)) -> Result<Picture, ComicError> {
        let pages = self.visible_pages(target);
        let viewport = self.memory.viewport;
        let rtl = self.memory.right_to_left;
        let first = self.decoded(pages[0])?;
        let result = if let Some(&second) = pages.get(1) {
            let first = first.clone();
            crate::viewport::spread(&first, self.decoded(second)?, target, rtl)
        } else {
            viewport.render(first, target)
        };
        result.map_err(|error| ComicError::Image(error.to_string()))
    }
    /// Makes one bounded preview while retaining the reading position.
    ///
    /// # Errors
    /// Returns the same page errors as normal reading.
    pub fn thumbnail(&mut self, index: usize) -> Result<Picture, ComicError> {
        self.decoded(index)?
            .fit(160, 240)
            .map_err(|error| ComicError::Image(error.to_string()))
    }
    #[must_use]
    pub fn cached_pages(&self) -> usize {
        self.cache.len()
    }
}
