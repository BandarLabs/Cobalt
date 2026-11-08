//! The Notifications list: everything the runtime has announced, kept so a
//! notice that came and went while the owner looked away can still be found.
//!
//! The runtime keeps the list and sends it here whenever it changes; this
//! only draws it. Nothing here is sent back until a list has arrived, which
//! is the runtime saying it keeps notices at all.

use kobo_sdk::clock::{Clock, Snapshot, SystemClock};
use kobo_sdk::{Context, Glyph, Notice, NoticeList, NoticeTarget, Screen, ScreenBuilder};

/// The action a row carries, by the notice's own identity.
pub fn opening(id: u64) -> String {
    format!("notice-{id}")
}

pub const OPEN_LIST: &str = "notifications";
pub const CLEAR: &str = "clear-notifications";
pub const CLEAR_CONFIRMED: &str = "clear-notifications-confirmed";
pub const KEEP: &str = "keep-notifications";
pub const PREVIOUS: &str = "notifications-previous";
pub const NEXT: &str = "notifications-next";

/// What the bar says about notices, beside the bell: how many are new, in
/// words, or nothing. A number in words rather than a dot, because a dot on
/// a grey panel is a smudge and says nothing about how many.
pub fn unread_label(list: &NoticeList) -> Option<String> {
    match list.unread() {
        0 => None,
        count => Some(format!("{count} new")),
    }
}

/// The reader's clock now, for saying when a notice arrived.
fn now() -> Option<Snapshot> {
    SystemClock::reader().now().ok()
}

/// The day a notice belongs under, and the time of day to put on its row:
/// "Today" and "08:13"; "Yesterday" and nothing, because by then the day is
/// the news; "3 Nov" and nothing.
fn said(at: u64, now: Option<Snapshot>) -> (String, Option<String>) {
    let spoken = now.and_then(|now| kobo_sdk::when::spoken(at, now));
    match spoken {
        Some(spoken) => match spoken.split_once(", ") {
            Some((day, time)) => (day.to_owned(), Some(time.to_owned())),
            None => (spoken, None),
        },
        None => ("Earlier".to_owned(), None),
    }
}

/// The muted line under a notice's title.
fn detail(notice: &Notice, time: Option<&str>) -> String {
    if notice.target == NoticeTarget::Gone {
        return "No longer on this reader".to_owned();
    }
    match (notice.detail.is_empty(), time) {
        (false, Some(time)) => format!("{}, {time}", notice.detail),
        (false, None) => notice.detail.clone(),
        (true, Some(time)) => time.to_owned(),
        (true, None) => notice.heading.clone(),
    }
}

/// One row as the list draws and measures it.
struct Line {
    section: Option<String>,
    id: u64,
    title: String,
    summary: String,
    glyph: Glyph,
    new: bool,
}

fn lines(list: &NoticeList, fresh: &[u64], now: Option<Snapshot>) -> Vec<Line> {
    let mut last_day: Option<String> = None;
    list.entries
        .iter()
        .map(|notice| {
            let (day, time) = said(notice.at, now);
            let section = (last_day.as_deref() != Some(day.as_str())).then(|| day.clone());
            last_day = Some(day);
            Line {
                section,
                id: notice.id,
                title: notice.title.clone(),
                summary: detail(notice, time.as_deref()),
                glyph: notice.glyph,
                new: notice.unread || fresh.contains(&notice.id),
            }
        })
        .collect()
}

/// Everything on the list's screen that is not a row, for measuring how many
/// rows a page holds.
fn frame() -> ScreenBuilder {
    ScreenBuilder::new("launcher-notifications")
        .top_bar("Notifications")
        .owns_back(true)
        .bottom_action(CLEAR, "Clear all")
}

/// The pages of the list, as row indices.
pub fn pages(context: &Context, list: &NoticeList, fresh: &[u64]) -> Vec<Vec<usize>> {
    let lines = lines(list, fresh, now());
    let measured: Vec<(Option<&str>, &str, &str)> = lines
        .iter()
        .map(|line| {
            (
                line.section.as_deref(),
                line.title.as_str(),
                line.summary.as_str(),
            )
        })
        .collect();
    if measured.is_empty() {
        return vec![Vec::new()];
    }
    let pages = context.paginate_rows_in_sections_under(
        &measured,
        false,
        kobo_sdk::Position::AtTheFoot,
        &frame().build(),
    );
    if pages.is_empty() {
        vec![Vec::new()]
    } else {
        pages
    }
}

/// The list, on page `page`. `fresh` is what was new when the owner opened
/// it, which keeps its mark for this visit though the runtime has already
/// been told it was seen.
pub fn screen(
    context: &Context,
    list: &NoticeList,
    fresh: &[u64],
    page: usize,
    asking: bool,
    trouble: Option<&str>,
) -> Screen {
    if list.entries.is_empty() {
        return ScreenBuilder::new("launcher-notifications")
            .top_bar("Notifications")
            .owns_back(true)
            .splash(
                Some(Glyph::Bell),
                "Nothing new",
                "Articles you send from your computer show up here.",
            )
            .build();
    }
    let lines = lines(list, fresh, now());
    let pages = pages(context, list, fresh);
    let page = page.min(pages.len() - 1);
    let mut screen = frame();
    if let Some(trouble) = trouble {
        screen = screen.banner(kobo_sdk::BannerLevel::Info, trouble);
    }
    let mut remaining = pages[page].as_slice();
    while let Some((&first, rest)) = remaining.split_first() {
        if let Some(section) = &lines[first].section {
            screen = screen.section(section.clone());
        }
        let count = rest
            .iter()
            .position(|&index| lines[index].section.is_some())
            .map_or(remaining.len(), |position| position + 1);
        screen = screen.rows_with_trailing(remaining[..count].iter().map(|&index| {
            let line = &lines[index];
            (
                opening(line.id),
                line.title.clone(),
                line.summary.clone(),
                line.glyph,
                if line.new { "New" } else { "" },
            )
        }));
        remaining = &remaining[count..];
    }
    if pages.len() > 1 {
        screen = screen.page_turns(PREVIOUS, NEXT).page_position(
            u16::try_from(page + 1).unwrap_or(u16::MAX),
            u16::try_from(pages.len()).unwrap_or(u16::MAX),
        );
    }
    if asking {
        screen = screen.dialog("Clear all notifications?", KEEP, |dialog| {
            dialog
                .text("The articles stay on the reader. Only this list is cleared.")
                .button(CLEAR_CONFIRMED, "Clear all")
                .button(KEEP, "Keep")
        });
    }
    screen.build()
}
