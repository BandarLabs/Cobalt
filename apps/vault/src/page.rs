//! Breaks a note into pages that hold headings as well as prose.
//!
//! The SDK's reading paginator takes a string, and a string has no headings:
//! every line is measured at body size. A heading is larger, so a page
//! measured that way and drawn with its headings set as headings runs off the
//! foot of the panel and loses its last lines. This measures each heading the
//! way the layout engine will draw it and each paragraph line by line, so a
//! page that fits here is the page that is drawn.

use crate::md::Block;
use kobo_sdk::{Context, ScreenBuilder};
use kobo_ui::{Chrome, DisplayMetrics, Face, FontSize, ProseArea};

/// One heading or one stretch of a paragraph, as it sits on one page.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Piece {
    Heading {
        level: u8,
        text: String,
    },
    Body {
        text: String,
        /// Link targets with the byte ranges of their words in `text`.
        links: Vec<(String, usize, usize)>,
    },
}

/// The fewest lines of a paragraph worth leaving alone at a page edge, the
/// same rule the SDK's own paginator keeps.
const MIN_KEEP_LINES: usize = 2;

/// The most pages one note is broken into, so a note of nothing but blank
/// headings cannot ask for an unbounded number of them.
const MAX_PAGES: usize = 4096;

pub fn paginate(blocks: &[Block], context: &Context) -> Vec<Vec<Piece>> {
    let metrics = context.metrics();
    kobo_ui::with_text_scale(metrics.text_scale, || {
        kobo_ui::with_reading_scale(metrics.text_scale, || Pager::new(metrics).run(blocks))
    })
}

struct Pager {
    metrics: DisplayMetrics,
    area: ProseArea,
    line: i32,
    pages: Vec<Vec<Piece>>,
    page: Vec<Piece>,
    used: i32,
}

impl Pager {
    fn new(metrics: DisplayMetrics) -> Self {
        // The reading screen's own area: the bars above and below, the status
        // band the runtime draws over everything, and the strip the page
        // position takes, which is reserved whether or not one is drawn.
        let mut area = metrics.prose_area_in(true, true, Face::Reading);
        area.height = area
            .height
            .saturating_sub(metrics.status_band_height())
            .saturating_sub(metrics.page_position_band())
            .max(1);
        Self {
            metrics,
            area,
            line: FontSize::Body.line_height_in(Face::Reading).max(1),
            pages: Vec::new(),
            page: Vec::new(),
            used: 0,
        }
    }

    fn run(mut self, blocks: &[Block]) -> Vec<Vec<Piece>> {
        for (index, block) in blocks.iter().enumerate() {
            if self.pages.len() >= MAX_PAGES {
                break;
            }
            match block {
                Block::Heading { level, text } => {
                    let follows = match blocks.get(index + 1) {
                        Some(Block::Body { text, .. }) => {
                            let lines = self.ranges(text).len().min(MIN_KEEP_LINES);
                            i32::try_from(lines).unwrap_or(0) * self.line
                        }
                        _ => 0,
                    };
                    self.heading(*level, text, follows);
                }
                Block::Body { text, links } => self.body(text, links),
            }
        }
        if !self.page.is_empty() {
            self.pages.push(self.page);
        }
        self.pages
    }

    fn gap(&self) -> i32 {
        if self.page.is_empty() {
            0
        } else {
            self.area.gap
        }
    }

    fn turn(&mut self) {
        if !self.page.is_empty() {
            self.pages.push(std::mem::take(&mut self.page));
        }
        self.used = 0;
    }

    /// How tall the layout engine draws this heading, asked of the engine
    /// itself rather than worked out here, so the two can never disagree.
    fn heading_height(&self, level: u8, text: &str) -> i32 {
        ScreenBuilder::new("vault-measure")
            .top_bar("Vault")
            .reading(true)
            .heading_at_level(level, text)
            .build()
            .layout_with(&self.metrics, &Chrome::measuring(true))
            .content_used()
    }

    /// Places a heading, keeping it on the same page as the first lines of
    /// the section it opens: a heading alone at the foot of a page promises
    /// a section and then turns the page on it.
    fn heading(&mut self, level: u8, text: &str, follows: i32) {
        let height = self.heading_height(level, text);
        let after = if follows > 0 {
            self.area.gap + follows
        } else {
            0
        };
        if !self.page.is_empty() && self.used + self.gap() + height + after > self.area.height {
            self.turn();
        }
        self.used += self.gap() + height;
        self.page.push(Piece::Heading {
            level,
            text: text.to_owned(),
        });
    }

    /// Where each line of a paragraph starts and ends in its own bytes.
    ///
    /// Wrapping hands back the lines as copies, each one a slice of the
    /// paragraph in order, so finding each from where the last one ended
    /// recovers the offsets that let a link's range follow it onto a page.
    fn ranges(&self, text: &str) -> Vec<(usize, usize)> {
        let mut ranges = Vec::new();
        let mut cursor = 0;
        for line in kobo_ui::wrap_text_in(text, self.area.width, FontSize::Body, self.area.face) {
            if line.is_empty() {
                continue;
            }
            let Some(found) = text[cursor..].find(&line) else {
                break;
            };
            let start = cursor + found;
            cursor = start + line.len();
            ranges.push((start, cursor));
        }
        ranges
    }

    fn body(&mut self, text: &str, links: &[(String, usize, usize)]) {
        let mut rows = self.ranges(text);
        while !rows.is_empty() {
            let spacing = self.gap();
            let room = self.area.height - self.used - spacing;
            let fits = usize::try_from((room / self.line).max(0)).unwrap_or(0);
            if fits == 0 {
                if self.page.is_empty() {
                    // Not even one line fits an empty page; place one anyway
                    // rather than loop for ever.
                    let rest = rows.split_off(1);
                    self.place(text, links, &rows, spacing);
                    self.turn();
                    rows = rest;
                    continue;
                }
                self.turn();
                continue;
            }
            if fits >= rows.len() {
                self.place(text, links, &rows, spacing);
                break;
            }
            let keep = fits.min(rows.len().saturating_sub(MIN_KEEP_LINES));
            let keep = if self.page.is_empty() {
                keep.max(1)
            } else {
                keep
            };
            if keep >= MIN_KEEP_LINES || (self.page.is_empty() && keep > 0) {
                let rest = rows.split_off(keep);
                self.place(text, links, &rows, spacing);
                self.turn();
                rows = rest;
            } else {
                self.turn();
            }
        }
    }

    /// Puts these lines of a paragraph on the page, with every link whose
    /// words fall on them, cut to the part that does.
    fn place(
        &mut self,
        text: &str,
        links: &[(String, usize, usize)],
        rows: &[(usize, usize)],
        spacing: i32,
    ) {
        let (Some(first), Some(last)) = (rows.first(), rows.last()) else {
            return;
        };
        let (from, to) = (first.0, last.1);
        let measured = i32::try_from(rows.len()).unwrap_or(i32::MAX) * self.line;
        self.used += spacing + measured.max(kobo_ui::MIN_TEXT_HEIGHT);
        let links = links
            .iter()
            .filter(|(_, start, end)| *end > from && *start < to)
            .map(|(target, start, end)| {
                (
                    target.clone(),
                    (*start).max(from) - from,
                    (*end).min(to) - from,
                )
            })
            .collect();
        self.page.push(Piece::Body {
            text: text[from..to].to_owned(),
            links,
        });
    }
}
