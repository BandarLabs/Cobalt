//! The protocol 16 controls, measured with the faces the runtime draws them
//! in, at the largest text size, on a Clara held both ways.
//!
//! A control's footprint is fixed by its fields and the text size, so the
//! worst case is the largest size with the longest words an application is
//! likely to send. If these fit here they fit at every smaller size.

use crate::{action_id, BannerLevel, Chrome, FieldKind, RowState, Screen, ScreenBuilder};
use kobo_ui::{
    DiagnosticSeverity, DisplayMetrics, Glyph, LayoutKind, Orientation, TextScale, CLARA_BW_METRICS,
};

fn largest() -> [(&'static str, DisplayMetrics); 2] {
    let portrait = DisplayMetrics {
        text_scale: TextScale::Largest,
        ..CLARA_BW_METRICS
    };
    #[cfg(feature = "text")]
    kobo_text::install(CLARA_BW_METRICS).unwrap();
    [
        ("portrait", portrait),
        ("landscape", portrait.oriented(Orientation::Landscape)),
    ]
}

fn assert_fits(name: &str, screen: &Screen) {
    for (orientation, metrics) in largest() {
        let diagnostics = screen.diagnostics(&metrics, &Chrome::measuring(true));
        let errors = diagnostics
            .issues
            .iter()
            .filter(|issue| issue.severity == DiagnosticSeverity::Error)
            .collect::<Vec<_>>();
        assert!(errors.is_empty(), "{name} {orientation}: {errors:?}");
    }
}

fn with_bars(screen: ScreenBuilder) -> Screen {
    screen
        .nav_bar(Some(0), [("home", "Home"), ("more", "More")])
        .build()
}

#[test]
fn toggles_with_long_names_fit_at_the_largest_text_size() {
    let screen = ScreenBuilder::new("toggles")
        .top_bar("Reading settings")
        .toggle(
            "sleep",
            "Put the reader to sleep when the cover is closed",
            "Wakes it again the moment the cover opens.",
            true,
        )
        .toggle("light", "Night light", "", false)
        .toggle_with_state(
            "sync",
            "Keep reading positions in step with the computer",
            "Needs Wi-Fi.",
            false,
            kobo_ui::ControlState::Disabled,
        );
    assert_fits("toggles", &with_bars(screen));
}

#[test]
fn ticked_rows_and_a_selection_bar_fit_at_the_largest_text_size() {
    let screen = ScreenBuilder::new("species")
        .top_bar("Species")
        .checked_rows([
            (
                "wren",
                "Eurasian wren",
                "Troglodytes troglodytes",
                Glyph::Book,
                RowState::Checked,
            ),
            (
                "robin",
                "European robin",
                "Erithacus rubecula",
                Glyph::Book,
                RowState::Unchecked,
            ),
            (
                "tits",
                "Tits, chickadees and titmice",
                "Paridae",
                Glyph::Folder,
                RowState::Mixed,
            ),
        ])
        .fill()
        .selection_bar(
            2,
            [
                ("delete", "Delete"),
                ("move", "Move to a list"),
                ("all", "Select all"),
            ],
        );
    assert_fits("checked rows", &with_bars(screen));
}

#[test]
fn rows_with_a_button_a_badge_a_value_and_a_menu_fit_at_the_largest_text_size() {
    let screen = ScreenBuilder::new("store")
        .top_bar("Updates")
        .rows_with_menu([
            (
                "birds",
                "Field guide to the birds of Britain and Europe",
                "Version 1.4 is ready",
                Glyph::Book,
                "birds-menu",
            ),
            ("moths", "Moths", "Installed", Glyph::Book, ""),
        ])
        .row_button("birds", "birds-update", "Update")
        .row_badge("birds", 1200)
        .row_button("moths", "moths-open", "Open");
    assert_fits("row button", &with_bars(screen));
}

#[test]
fn a_field_with_a_label_a_two_line_hint_and_an_error_fits_at_the_largest_text_size() {
    let screen = ScreenBuilder::new("token")
        .top_bar("Lichess")
        .field("token", "fake-token-abcdefghijklmn", "Paste your token")
        .field_label("Lichess token")
        .field_hint("24 characters, from lichess.org/account/oauth/token")
        .field_error("Lichess refused this token. Make a new one and paste it here.")
        .field_kind(FieldKind::Secret)
        .field_clear("token-clear")
        .primary_button("save", "Save");
    assert_fits("field", &with_bars(screen));
}

#[test]
fn banners_of_every_level_with_an_action_fit_at_the_largest_text_size() {
    let screen = ScreenBuilder::new("banners")
        .top_bar("Store")
        .banner(
            BannerLevel::Error,
            "The Store did not answer. Your apps are still on the reader.",
        )
        .banner_action("retry", "Check again")
        .banner(BannerLevel::Success, "Signed in.");
    assert_fits("banners", &with_bars(screen));
}

#[test]
fn every_page_of_a_long_option_list_fits_under_both_bars_at_the_largest_text_size() {
    for (orientation, metrics) in largest() {
        let count = 40;
        let pages = kobo_ui::options_pages(count, &metrics, true);
        assert!(pages > 1, "{orientation}");
        for page in 0..pages {
            let screen = ScreenBuilder::new("zones")
                .top_bar("Time zone")
                .option_list(
                    "Choose the zone this reader keeps",
                    u16::try_from(page).unwrap(),
                    "zones-previous",
                    "zones-next",
                    (0..count).map(|index| {
                        (
                            format!("zone-{index}"),
                            format!("America/Argentina/ComodRivadavia {index}"),
                            format!("Three hours behind Greenwich, all year {index}"),
                        )
                    }),
                )
                .chosen(page * 3);
            let screen = with_bars(screen);
            let diagnostics = screen.diagnostics(&metrics, &Chrome::measuring(true));
            assert!(
                !diagnostics.has_errors(),
                "{orientation} page {page}: {:?}",
                diagnostics.issues
            );
            let layout = screen.layout_with(&metrics, &Chrome::measuring(true));
            let rows = layout
                .nodes
                .iter()
                .filter(|node| matches!(node.kind, LayoutKind::OptionRow(..)))
                .count();
            assert!(rows >= 1, "{orientation} page {page}");
        }
    }
}

#[test]
fn the_longest_code_with_a_two_line_caption_fits_under_both_bars_at_the_largest_text_size() {
    let payload = format!(
        "https://lichess.org/{}",
        "a".repeat(kobo_ui::MAX_CODE_BYTES - 20)
    );
    assert_eq!(payload.len(), kobo_ui::MAX_CODE_BYTES);
    let screen = ScreenBuilder::new("code").top_bar("Sign in").code(
        payload,
        "Scan this with your phone to sign in at lichess.org, then come back to the reader.",
    );
    assert_fits("code", &with_bars(screen));
}

#[test]
fn a_step_indicator_with_a_long_step_name_fits_at_the_largest_text_size() {
    let screen = ScreenBuilder::new("setup")
        .top_bar("Set up")
        .steps(
            1,
            4,
            [
                "Account",
                "Sign in at lichess.org with the phone beside you",
                "Board",
                "Done",
            ],
        )
        .text("Scan the code to sign in.");
    assert_fits("steps", &with_bars(screen));
}

#[test]
fn every_new_builder_names_its_actions_so_drive_scripts_can_tap_them() {
    let screen = ScreenBuilder::new("names")
        .toggle("sleep", "Sleep", "", true)
        .checked_rows([("wren", "Wren", "", Glyph::Book, RowState::Checked)])
        .row_button("wren", "wren-open", "Open")
        .selection_bar(1, [("delete", "Delete")])
        .banner(BannerLevel::Info, "Offline")
        .banner_action("retry", "Retry")
        .option_list("Zone", 0, "previous", "next", [("utc", "UTC", "")])
        .build();
    let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
    for name in ["sleep", "wren", "wren-open", "delete", "retry", "utc"] {
        assert!(
            layout.rect_of_action(action_id(name)).is_some(),
            "nothing on the panel answers to {name}"
        );
    }
}
