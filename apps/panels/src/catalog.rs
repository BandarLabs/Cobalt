//! Measured catalog pages preserve the server's original action identities.
use super::{Context, Glyph, Panels, Screen, ScreenBuilder};

type Row = (String, String, String, Glyph);

pub(super) fn preview(text: &str, limit: usize) -> String {
    let mut chars = text.chars();
    let mut short: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        short.push('…');
    }
    short
}

impl Panels {
    pub(super) fn follow_catalog(&mut self, context: &mut Context, url: String) {
        if self.history.len() >= 16 {
            self.notice = Some("Go back before opening another library page.".into());
            return;
        }
        if let Some(feed) = self.catalog.take() {
            self.history.push((
                self.catalog_url.clone(),
                feed,
                self.catalog_page,
                self.query.clone(),
            ));
        }
        self.query.clear();
        self.fetch_catalog(context, url);
    }

    pub(super) fn catalog_rows(&self) -> Vec<Row> {
        let Some(feed) = &self.catalog else {
            return Vec::new();
        };
        let query = self.query.to_lowercase();
        let matches = |title: &str| query.is_empty() || title.to_lowercase().contains(&query);
        let mut rows: Vec<_> = feed
            .navigation
            .iter()
            .enumerate()
            .filter(|(_, item)| matches(&item.title))
            .map(|(index, item)| {
                (
                    format!("section-{index}"),
                    preview(&item.title, 60),
                    preview(item.summary.as_deref().unwrap_or("Open section"), 80),
                    Glyph::Reader,
                )
            })
            .chain(
                feed.publications
                    .iter()
                    .enumerate()
                    .filter(|(_, item)| matches(&item.title))
                    .map(|(index, item)| {
                        (
                            format!("volume-{index}"),
                            preview(&item.title, 60),
                            preview(&item.authors.join(", "), 80),
                            Glyph::Book,
                        )
                    }),
            )
            .collect();
        if feed.previous().is_some() {
            rows.push((
                "catalog-previous".into(),
                "Previous results".into(),
                "Load another library page".into(),
                Glyph::Reader,
            ));
        }
        if feed.next().is_some() {
            rows.push((
                "catalog-next".into(),
                "More results".into(),
                "Load another library page".into(),
                Glyph::Reader,
            ));
        }
        rows
    }

    pub(super) fn catalog_pages(&self, context: &Context) -> Vec<Vec<usize>> {
        let rows = self.catalog_rows();
        let measured = rows
            .iter()
            .map(|(_, title, summary, _)| (title.as_str(), summary.as_str(), ""))
            .collect::<Vec<_>>();
        context.paginate_rows_below_section(
            &measured,
            true,
            kobo_sdk::Position::AtTheFoot,
            self.notice.as_deref(),
        )
    }

    pub(super) fn catalog_screen(&self, context: &Context) -> Screen {
        let title = self
            .catalog
            .as_ref()
            .and_then(|feed| feed.title.as_deref())
            .unwrap_or("Home library");
        let mut base = ScreenBuilder::new("panels-catalog")
            .top_bar(preview(title, 60))
            .owns_back(true);
        if self.catalog.is_some() {
            base = base.top_bar_glyph("search", "Search", Glyph::Search);
        }
        let screen = self.with_notice(base);
        if self.catalog.is_none() {
            return if self.notice.is_some() {
                screen
                    .splash(
                        Some(Glyph::Reader),
                        "Library unavailable",
                        "Return to Browse to check the server address and account details.",
                    )
                    .primary_button("retry-catalog", "Try again")
                    .build()
            } else {
                screen.activity("Opening library", None).build()
            };
        }
        let rows = self.catalog_rows();
        if rows.is_empty() {
            return screen
                .splash(
                    Some(Glyph::Reader),
                    if self.query.is_empty() {
                        "No comics here yet"
                    } else {
                        "No matching titles"
                    },
                    if self.query.is_empty() {
                        "Try another section of your library."
                    } else {
                        "Search with fewer words or return to your library."
                    },
                )
                .build();
        }
        let pages = self.catalog_pages(context);
        let page = self.catalog_page.min(pages.len().saturating_sub(1));
        screen
            .section(if self.query.is_empty() {
                "Browse"
            } else {
                "Search results"
            })
            .rows(
                pages
                    .get(page)
                    .into_iter()
                    .flatten()
                    .map(|&index| rows[index].clone()),
            )
            .page_turns("catalog-page-back", "catalog-page-next")
            .page_position(
                u16::try_from(page + 1).unwrap_or(u16::MAX),
                u16::try_from(pages.len().max(1)).unwrap_or(u16::MAX),
            )
            .build()
    }
}
