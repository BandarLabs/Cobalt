use super::{builtin_mark, room_on, Notices, Opens, Room, INBOX, SHOWN_FOR_MS};
use kobo_protocol::NoticeTarget;
use kobo_ui::Glyph;
use std::fs;
use std::path::{Path, PathBuf};

const NOW: u64 = 1_762_600_000_000;

// Three to five seconds: long enough to read, short enough not to wait for.
const _: () = assert!(SHOWN_FOR_MS >= 3_000 && SHOWN_FOR_MS <= 5_000);

fn folder(name: &str) -> PathBuf {
    let folder = std::env::temp_dir().join(format!("cobalt-notices-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&folder);
    fs::create_dir_all(folder.join(INBOX)).expect("an inbox");
    folder
}

fn drop_in(folder: &Path, name: &str, record: &str) {
    fs::write(folder.join(INBOX).join(name), record).expect("a record");
}

fn article(title: &str) -> String {
    format!(
        r#"{{"kind":"article","title":"{title}","source":"example.com","app":"books","path":"Articles/{title}.epub"}}"#
    )
}

#[test]
fn an_article_dropped_in_the_inbox_is_kept_and_announced_by_its_title() {
    let folder = folder("one");
    drop_in(&folder, "1.json", &article("River"));
    let mut notices = Notices::open(&folder);
    assert_eq!(notices.collect(NOW), (1, 0));
    assert!(fs::read_dir(folder.join(INBOX))
        .expect("inbox")
        .next()
        .is_none());
    let shown = notices.announce(false, builtin_mark).expect("a card");
    assert_eq!(shown.card.heading, "Article arrived");
    assert_eq!(shown.card.line, "River");
    assert_eq!(shown.card.glyph, Glyph::Book);
    assert!(matches!(shown.opens, Opens::One(_)));
    // Said once: the next pass has nothing to announce.
    assert!(notices.announce(false, builtin_mark).is_none());
    assert_eq!(notices.unread(), 1);
}

#[test]
fn several_notices_arriving_together_collapse_into_one_card_that_opens_the_list() {
    let folder = folder("several");
    for (name, title) in [("1.json", "River"), ("2.json", "Hills"), ("3.json", "Sea")] {
        drop_in(&folder, name, &article(title));
    }
    let mut notices = Notices::open(&folder);
    assert_eq!(notices.collect(NOW).0, 3);
    let shown = notices.announce(false, builtin_mark).expect("one card");
    assert_eq!(shown.card.heading, "3 articles arrived");
    assert!(
        shown.card.line.ends_with(" and 2 more"),
        "{}",
        shown.card.line
    );
    assert_eq!(shown.opens, Opens::List);
    assert!(notices.announce(false, builtin_mark).is_none());
}

#[test]
fn notices_that_arrived_while_the_reader_slept_say_so_on_one_card() {
    let folder = folder("asleep");
    drop_in(&folder, "1.json", &article("River"));
    drop_in(&folder, "2.json", &article("Hills"));
    let mut notices = Notices::open(&folder);
    notices.collect(NOW);
    let shown = notices.announce(true, builtin_mark).expect("one card");
    assert_eq!(shown.card.heading, "2 articles arrived");
    assert_eq!(shown.card.line, "While the reader was asleep");
}

#[test]
fn news_of_different_kinds_collapses_without_calling_it_all_articles() {
    let folder = folder("mixed");
    drop_in(&folder, "1.json", &article("River"));
    drop_in(
        &folder,
        "2.json",
        r#"{"kind":"update","heading":"Update installed","title":"Daily Brief 1.2","app":"store"}"#,
    );
    let mut notices = Notices::open(&folder);
    notices.collect(NOW);
    let shown = notices.announce(false, builtin_mark).expect("one card");
    assert_eq!(shown.card.heading, "2 new notifications");
    assert_eq!(shown.card.glyph, Glyph::Bell);
}

#[test]
fn the_list_survives_a_restart_with_what_was_read_and_what_was_not() {
    let folder = folder("restart");
    drop_in(&folder, "1.json", &article("River"));
    let mut notices = Notices::open(&folder);
    notices.collect(NOW);
    assert!(notices.mark_read());
    drop_in(&folder, "2.json", &article("Hills"));
    notices.collect(NOW + 1_000);
    drop(notices);

    let reopened = Notices::open(&folder);
    let list = reopened.list(&folder, false, builtin_mark);
    let titles: Vec<&str> = list
        .entries
        .iter()
        .map(|notice| notice.title.as_str())
        .collect();
    assert_eq!(titles, ["Hills", "River"], "newest first");
    assert_eq!(
        list.entries
            .iter()
            .map(|notice| notice.unread)
            .collect::<Vec<_>>(),
        [true, false]
    );
    assert_eq!(list.entries[0].heading, "Article arrived");
    assert_eq!(list.entries[0].detail, "example.com");
    assert_eq!(list.entries[0].at, NOW + 1_000);
    // Nothing is announced again after a restart: it was announced before.
    let mut reopened = reopened;
    assert!(reopened.announce(false, builtin_mark).is_none());
    // A new notice after the restart does not reuse an old one's identity.
    drop_in(&folder, "3.json", &article("Sea"));
    reopened.collect(NOW + 2_000);
    let ids: Vec<u64> = reopened
        .list(&folder, false, builtin_mark)
        .entries
        .iter()
        .map(|notice| notice.id)
        .collect();
    assert_eq!(ids.len(), 3);
    assert!(ids[0] > ids[1] && ids[1] > ids[2], "{ids:?}");
}

#[test]
fn clearing_the_list_is_kept_across_a_restart_too() {
    let folder = folder("clear");
    drop_in(&folder, "1.json", &article("River"));
    let mut notices = Notices::open(&folder);
    notices.collect(NOW);
    notices.clear();
    assert!(notices.announce(false, builtin_mark).is_none());
    assert!(Notices::open(&folder)
        .list(&folder, false, builtin_mark)
        .entries
        .is_empty());
}

#[test]
fn the_list_keeps_the_newest_hundred() {
    let folder = folder("cap");
    let mut notices = Notices::open(&folder);
    for index in 0..105_u64 {
        drop_in(
            &folder,
            &format!("{index:03}.json"),
            &article(&format!("Article {index}")),
        );
        if index % 30 == 29 {
            notices.collect(NOW + index);
        }
    }
    notices.collect(NOW + 200);
    let list = Notices::open(&folder).list(&folder, false, builtin_mark);
    assert_eq!(list.entries.len(), kobo_protocol::MAX_NOTICES);
    assert_eq!(list.entries[0].title, "Article 104");
    assert!(list
        .entries
        .iter()
        .all(|notice| notice.title != "Article 0"));
}

#[test]
fn a_file_that_is_not_a_notice_is_thrown_away_and_a_half_written_one_is_left_alone() {
    let folder = folder("bad");
    drop_in(&folder, "1.json", "not json");
    drop_in(&folder, "2.json", r#"{"kind":"article","title":"  "}"#);
    drop_in(
        &folder,
        "3.json",
        r#"{"kind":"update","title":"No heading"}"#,
    );
    drop_in(
        &folder,
        "4.json",
        &format!(r#"{{"title":"{}"}}"#, "x".repeat(5000)),
    );
    drop_in(&folder, "5.json.part", &article("Half"));
    drop_in(&folder, ".6.json", &article("Hidden"));
    let mut notices = Notices::open(&folder);
    assert_eq!(notices.collect(NOW), (0, 4));
    assert!(folder.join(INBOX).join("5.json.part").exists());
    assert!(folder.join(INBOX).join(".6.json").exists());
    assert!(notices.announce(false, builtin_mark).is_none());
}

#[test]
fn a_notice_whose_article_was_deleted_says_so_rather_than_offering_to_open_it() {
    let folder = folder("gone");
    let library = folder.join("card");
    fs::create_dir_all(library.join("Articles")).expect("a library");
    fs::write(library.join("Articles/River.epub"), b"epub").expect("an article");
    drop_in(&folder, "1.json", &article("River"));
    drop_in(&folder, "2.json", &article("Hills"));
    drop_in(&folder, "3.json", r#"{"title":"Just news"}"#);
    let mut notices = Notices::open(&folder);
    notices.collect(NOW);
    let list = notices.list(&library, false, builtin_mark);
    let target = |title: &str| {
        list.entries
            .iter()
            .find(|notice| notice.title == title)
            .map(|notice| (notice.id, notice.target))
            .expect("listed")
    };
    assert_eq!(target("River").1, NoticeTarget::Ready);
    assert_eq!(target("Hills").1, NoticeTarget::Gone);
    assert_eq!(target("Just news").1, NoticeTarget::Nothing);
    assert_eq!(
        notices.target(target("River").0, &library),
        Some(("books".to_owned(), "0/Articles/River.epub".to_owned()))
    );
    assert_eq!(notices.target(target("Hills").0, &library), None);
}

#[test]
fn a_path_that_climbs_out_of_the_card_is_never_kept() {
    let folder = folder("climb");
    for (name, path) in [("1.json", "../etc/passwd"), ("2.json", "/etc/passwd")] {
        drop_in(
            &folder,
            name,
            &format!(r#"{{"title":"Odd","path":"{path}"}}"#),
        );
    }
    let mut notices = Notices::open(&folder);
    notices.collect(NOW);
    let list = notices.list(Path::new("/"), false, builtin_mark);
    assert_eq!(list.entries.len(), 2);
    assert!(list
        .entries
        .iter()
        .all(|notice| notice.target == NoticeTarget::Nothing));
}

#[test]
fn a_time_from_a_clock_ahead_of_the_readers_is_read_as_now_and_seconds_as_seconds() {
    let folder = folder("time");
    drop_in(
        &folder,
        "1.json",
        r#"{"title":"Ahead","time":99999999999999}"#,
    );
    drop_in(
        &folder,
        "2.json",
        r#"{"title":"Seconds","time":1762500000}"#,
    );
    let mut notices = Notices::open(&folder);
    notices.collect(NOW);
    let list = notices.list(&folder, false, builtin_mark);
    let at = |title: &str| {
        list.entries
            .iter()
            .find(|notice| notice.title == title)
            .map(|notice| notice.at)
    };
    assert_eq!(at("Ahead"), Some(NOW));
    assert_eq!(at("Seconds"), Some(1_762_500_000_000));
}

#[test]
fn a_card_waits_for_a_keyboard_or_a_dialogue_and_never_goes_over_a_page_being_read() {
    let list = kobo_ui::Screen::new(1, Vec::new());
    assert_eq!(room_on(Some(&list), false), Room::Show);
    assert_eq!(room_on(Some(&list), true), Room::Hold);
    assert_eq!(room_on(None, false), Room::Hold);
    let mut asking = list.clone();
    asking.overlay = Some(Box::new(kobo_ui::Overlay::modal(
        kobo_ui::NodeId(9),
        "Delete it?",
        Vec::new(),
    )));
    assert_eq!(room_on(Some(&asking), false), Room::Hold);
    let mut page = list.clone();
    page.reading = true;
    assert_eq!(room_on(Some(&page), false), Room::Quiet);
    let mut comic = list;
    comic.auto_hide_top_bar = true;
    assert_eq!(room_on(Some(&comic), false), Room::Quiet);
}

#[test]
fn news_that_arrived_while_the_owner_was_reading_is_kept_but_never_put_on_a_card() {
    let folder = folder("quiet");
    drop_in(&folder, "1.json", &article("River"));
    let mut notices = Notices::open(&folder);
    notices.collect(NOW);
    notices.keep_quiet();
    assert!(notices.announce(false, builtin_mark).is_none());
    assert_eq!(notices.unread(), 1);
}

#[test]
fn news_arriving_while_a_card_is_up_joins_that_card_rather_than_queueing_behind_it() {
    let folder = folder("joins");
    drop_in(&folder, "1.json", &article("River"));
    let mut notices = Notices::open(&folder);
    notices.collect(NOW);
    let first = notices.announce(false, builtin_mark).expect("a card");
    assert_eq!(first.card.heading, "Article arrived");
    drop_in(&folder, "2.json", &article("Hills"));
    drop_in(&folder, "3.json", &article("Sea"));
    notices.collect(NOW + 500);
    let grown = notices
        .announce(false, builtin_mark)
        .expect("the card again");
    assert_eq!(grown.card.heading, "3 articles arrived");
    assert_eq!(grown.opens, Opens::List);
    // Once it has gone, the next arrival is a card of its own.
    notices.card_gone();
    drop_in(&folder, "4.json", &article("Fields"));
    notices.collect(NOW + 9_000);
    let next = notices.announce(false, builtin_mark).expect("a new card");
    assert_eq!(next.card.heading, "Article arrived");
    assert_eq!(next.card.line, "Fields");
}
