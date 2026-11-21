use std::sync::atomic::AtomicUsize;
use std::sync::Arc;

use super::super::testing::{picture, picture_address, LISTING, MANIFESTS};
use super::*;

/// A folder of kept pictures for one test, emptied first and named for it.
fn folder(name: &str) -> PathBuf {
    let folder =
        std::env::temp_dir().join(format!("kobo-ui-pictures-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&folder);
    folder
}

fn listed(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

/// A site that publishes `pictures` and counts how often it is asked.
fn site(pictures: &[(&str, Vec<u8>)]) -> (Box<Fetch>, Arc<AtomicUsize>) {
    let published: BTreeMap<String, Vec<u8>> = pictures
        .iter()
        .map(|(name, bytes)| (picture_address(name), bytes.clone()))
        .collect();
    let asked = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&asked);
    (
        Box::new(move |url, _| {
            counted.fetch_add(1, Ordering::SeqCst);
            published.get(url).cloned().ok_or(DeviceError::NotFound)
        }),
        asked,
    )
}

fn unreachable() -> (Box<Fetch>, Arc<AtomicUsize>) {
    let asked = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&asked);
    (
        Box::new(move |_, _| {
            counted.fetch_add(1, Ordering::SeqCst);
            Err(DeviceError::Unreachable)
        }),
        asked,
    )
}

/// Makes a kept picture look as if it was fetched `ago`.
fn age(path: &Path, ago: Duration) {
    let file = fs::File::options().write(true).open(path).expect("kept");
    file.set_modified(std::time::SystemTime::now() - ago)
        .expect("set its date");
}

#[test]
fn the_store_this_command_is_built_with_shows_every_app_once_in_the_websites_groups() {
    let store = Shopfront::bundled().expect("the bundled store");
    let answer = store.answer();
    let groups = answer["groups"].as_array().expect("groups");

    // No app is waiting for the listing to catch up with it.
    let ids: Vec<&str> = groups
        .iter()
        .filter_map(|group| group["id"].as_str())
        .collect();
    assert_eq!(
        ids,
        [
            "reading",
            "games",
            "everyday",
            "home-and-computer",
            "developers"
        ],
        "run node tools/store-listing.mjs"
    );
    let apps = answer["apps"].as_object().expect("apps");
    let mut seen = BTreeSet::new();
    for group in groups {
        let titles: Vec<String> = group["apps"]
            .as_array()
            .expect("ids")
            .iter()
            .map(|id| {
                let id = id.as_str().expect("an id");
                assert!(seen.insert(id.to_owned()), "{id} is listed twice");
                apps[id]["title"].as_str().expect("a title").to_lowercase()
            })
            .collect();
        assert!(titles.is_sorted(), "{titles:?}");
    }
    assert_eq!(seen.len(), apps.len());
    assert_eq!(apps.len(), kobo_catalog::sources().len());

    let lichess = &apps["lichess"];
    assert_eq!(lichess["title"], "Lichess");
    assert_eq!(lichess["shots"][0]["name"], "lichess.png");
    assert_eq!(lichess["shots"][0]["width"], 1072);
    assert!(lichess["shots"].as_array().expect("shots").len() > 1);
    // The words for what an app may use are the ones `kobo apps setup`
    // prints.
    assert!(lichess["access"]
        .as_array()
        .expect("access")
        .contains(&json!(kobo_policy::Capability::KeepAwake.plain_wording())));
    assert!(lichess["setup"]
        .as_array()
        .expect("setup")
        .iter()
        .any(|step| step["command"] == "kobo secret set lichess --from token.txt"));
    assert!(lichess["release_notes"].is_string());
    // Every listed picture is one this would agree to fetch.
    assert!(store.pictures().len() > 100);
    assert!(store.shows("lichess.png"));
}

#[test]
fn an_app_says_what_its_manifest_says_and_no_more_than_it_says() {
    let store = Shopfront::from(MANIFESTS, LISTING).expect("store");
    let almanac = &store.answer()["apps"]["almanac"];
    assert_eq!(
        *almanac,
        json!({
            "id": "almanac",
            "title": "Almanac",
            "summary": "The year at a glance.",
            "description": "Sunrise, sunset and the phase of the moon for every day of the year.",
            "version": "1.0.0",
            "release_notes": "The moon is drawn the right way up.",
            "access": ["Connects to online services", "Can keep the reader awake while in use"],
            "setup": [
                { "text": "Find your town.", "command": null,
                  "link": { "label": "Towns", "url": "https://example.org/towns" } },
                { "text": "Save it.", "command": "kobo almanac town NAME", "link": null },
            ],
            "shots": [
                { "name": "almanac.png", "caption": "The year on one screen", "width": 1072, "height": 1448 },
                { "name": "almanac/moon.png", "caption": "Tonight's moon", "width": 1072, "height": 1448 },
            ],
        })
    );
    // With nothing longer written, the one line it has is its description.
    let tides = &store.answer()["apps"]["tide-table"];
    assert_eq!(tides["description"], "High and low water.");
    assert_eq!(tides["release_notes"], Value::Null);
    assert_eq!(tides["access"], json!([]));
    assert_eq!(tides["setup"], json!([]));
    assert_eq!(tides["shots"], json!([]));
    assert!(store.shows("almanac/moon.png"));
    assert!(!store.shows("almanac/sun.png"));
}

#[test]
fn an_app_the_listing_has_not_caught_up_with_is_still_shown_without_pictures() {
    let listing = r#"{ "groups": [{ "id": "reading", "title": "Reading", "apps": ["almanac", "gone"] }],
                       "shots": {}, "release_note_overrides": {} }"#;
    let store = Shopfront::from(MANIFESTS, listing).expect("store");
    assert_eq!(
        store.answer()["groups"],
        json!([
            { "id": "reading", "title": "Reading", "apps": ["almanac"] },
            { "id": "other", "title": "Other apps", "apps": ["tide-table"] },
        ])
    );
    assert_eq!(store.answer()["apps"]["almanac"]["shots"], json!([]));
    assert!(store.pictures().is_empty());
}

#[test]
fn a_release_note_the_website_hides_or_rewords_is_hidden_or_reworded_here() {
    let hidden = LISTING.replace(
        r#""release_note_overrides": {}"#,
        r#""release_note_overrides": { "almanac@1.0.0": null, "tide-table@3.1.0": "Low water is marked." }"#,
    );
    let store = Shopfront::from(MANIFESTS, &hidden).expect("store");
    assert_eq!(
        store.answer()["apps"]["almanac"]["release_notes"],
        Value::Null
    );
    assert_eq!(
        store.answer()["apps"]["tide-table"]["release_notes"],
        "Low water is marked."
    );
    // An override is for one version and lapses with it.
    let lapsed = LISTING.replace(
        r#""release_note_overrides": {}"#,
        r#""release_note_overrides": { "almanac@0.9.0": null }"#,
    );
    let store = Shopfront::from(MANIFESTS, &lapsed).expect("store");
    assert_eq!(
        store.answer()["apps"]["almanac"]["release_notes"],
        "The moon is drawn the right way up."
    );
}

#[test]
fn a_listing_cannot_name_a_picture_that_is_a_path_or_an_address_of_its_own() {
    for name in [
        "../almanac.png",
        "almanac/../../secret.png",
        "/etc/almanac.png",
        "a/b/c.png",
        "almanac/.png",
        "Almanac.png",
        "almanac.jpg",
        "almanac.png?x=1",
        "https://example.org/almanac.png",
        "almanac_moon.png",
        "-almanac.png",
        ".png",
        "",
    ] {
        assert!(!is_a_picture_name(name), "{name}");
        let listing = LISTING.replace("almanac/moon.png", name);
        assert!(Shopfront::from(MANIFESTS, &listing).is_err(), "{name}");
    }
    for name in [
        "almanac.png",
        "almanac/moon.png",
        "rss-miniflux/2-feeds.png",
    ] {
        assert!(is_a_picture_name(name), "{name}");
    }
}

#[test]
fn a_picture_is_fetched_once_and_served_from_this_computer_from_then_on() {
    let folder = folder("once");
    let pictures = Pictures::open(
        folder.clone(),
        &listed(&["almanac.png", "almanac/moon.png"]),
    );
    let moon = picture(1072, 1448, 2);
    let (fetch, asked) = site(&[("almanac/moon.png", moon.clone())]);

    assert_eq!(
        pictures.get("almanac/moon.png", &*fetch),
        Some(moon.clone())
    );
    assert_eq!(
        pictures.get("almanac/moon.png", &*fetch),
        Some(moon.clone())
    );
    assert_eq!(asked.load(Ordering::SeqCst), 1);
    // Kept under a name with no folder in it, and nothing left beside it.
    assert_eq!(
        fs::read_dir(&folder)
            .expect("folder")
            .map(|entry| entry
                .expect("entry")
                .file_name()
                .into_string()
                .expect("name"))
            .collect::<Vec<_>>(),
        ["almanac_moon.png"]
    );

    // A later run, with no network at all, still has it.
    let later = Pictures::open(
        folder.clone(),
        &listed(&["almanac.png", "almanac/moon.png"]),
    );
    let (nothing, asked) = unreachable();
    assert_eq!(later.get("almanac/moon.png", &*nothing), Some(moon));
    assert_eq!(asked.load(Ordering::SeqCst), 0);
    let _ = fs::remove_dir_all(folder);
}

#[test]
fn what_arrives_is_kept_and_served_only_if_it_opens_as_a_picture_of_a_sane_size() {
    let folder = folder("sane");
    let names = ["page.png", "short.png", "vast.png", "empty.png", "fine.png"];
    let pictures = Pictures::open(folder.clone(), &listed(&names));
    let mut vast = picture(1072, 1448, 0);
    vast[16..20].copy_from_slice(&100_000_u32.to_be_bytes());
    let (fetch, _) = site(&[
        (
            "page.png",
            b"<!doctype html><title>Sign in to the hotel Wi-Fi</title>".to_vec(),
        ),
        ("short.png", picture(1072, 1448, 0)[..20].to_vec()),
        ("vast.png", vast),
        ("empty.png", picture(0, 1448, 0)),
        ("fine.png", picture(LONGEST_SIDE, 1, 0)),
    ]);

    for name in &names[..4] {
        assert_eq!(pictures.get(name, &*fetch), None, "{name}");
        assert!(!folder.join(name).exists(), "{name} was kept");
    }
    assert!(pictures.get("fine.png", &*fetch).is_some());

    // A kept file that has stopped being a picture is not served either,
    // and is replaced by a fetch.
    fs::write(folder.join("fine.png"), b"not a picture any more").expect("spoil");
    assert_eq!(
        pictures.get("fine.png", &*fetch),
        Some(picture(LONGEST_SIDE, 1, 0))
    );
    assert!(is_a_picture(
        &fs::read(folder.join("fine.png")).expect("kept")
    ));
    let _ = fs::remove_dir_all(folder);
}

#[test]
fn a_site_that_cannot_be_reached_is_asked_once_and_then_left_alone_for_a_while() {
    let folder = folder("rest");
    let names = ["a.png", "b.png", "c.png"];
    let mut pictures = Pictures::open(folder.clone(), &listed(&names));
    pictures.rest = Duration::from_millis(200);
    let (nothing, asked) = unreachable();

    for name in names {
        assert_eq!(pictures.get(name, &*nothing), None);
    }
    assert_eq!(
        asked.load(Ordering::SeqCst),
        1,
        "one failure answers for the rest"
    );

    // When the rest is over the site is tried again, and one that answers
    // ends it for every picture.
    std::thread::sleep(Duration::from_millis(250));
    let (fetch, asked) = site(&[("a.png", picture(10, 10, 1)), ("b.png", picture(10, 10, 2))]);
    assert_eq!(pictures.get("a.png", &*fetch), Some(picture(10, 10, 1)));
    assert_eq!(pictures.get("b.png", &*fetch), Some(picture(10, 10, 2)));
    assert_eq!(asked.load(Ordering::SeqCst), 2);
    let _ = fs::remove_dir_all(folder);
}

#[test]
fn a_picture_the_site_does_not_have_is_not_asked_for_again_at_once_and_holds_up_no_other() {
    let folder = folder("refused");
    let mut pictures = Pictures::open(folder.clone(), &listed(&["gone.png", "here.png"]));
    pictures.rest = Duration::from_millis(200);
    let (fetch, asked) = site(&[("here.png", picture(10, 10, 1))]);

    assert_eq!(pictures.get("gone.png", &*fetch), None);
    assert_eq!(pictures.get("gone.png", &*fetch), None);
    assert_eq!(asked.load(Ordering::SeqCst), 1);
    assert!(pictures.get("here.png", &*fetch).is_some());
    assert_eq!(asked.load(Ordering::SeqCst), 2);

    std::thread::sleep(Duration::from_millis(250));
    assert_eq!(pictures.get("gone.png", &*fetch), None);
    assert_eq!(asked.load(Ordering::SeqCst), 3);
    let _ = fs::remove_dir_all(folder);
}

#[test]
fn an_old_kept_picture_is_replaced_when_the_site_answers_and_still_shown_when_it_does_not() {
    let folder = folder("old");
    let pictures = Pictures::open(folder.clone(), &listed(&["almanac.png"]));
    let (before, after) = (picture(1072, 1448, 1), picture(1072, 1448, 9));
    let (first, _) = site(&[("almanac.png", before.clone())]);
    assert_eq!(pictures.get("almanac.png", &*first), Some(before.clone()));
    age(&folder.join("almanac.png"), FRESH + Duration::from_secs(60));

    // Out of reach: the old one is better than an empty frame.
    let (nothing, asked) = unreachable();
    assert_eq!(pictures.get("almanac.png", &*nothing), Some(before));
    assert_eq!(asked.load(Ordering::SeqCst), 1);

    // In reach again, on a later run: the retaken picture replaces it.
    let pictures = Pictures::open(folder.clone(), &listed(&["almanac.png"]));
    let (retaken, asked) = site(&[("almanac.png", after.clone())]);
    assert_eq!(pictures.get("almanac.png", &*retaken), Some(after.clone()));
    assert_eq!(pictures.get("almanac.png", &*retaken), Some(after));
    assert_eq!(asked.load(Ordering::SeqCst), 1);
    let _ = fs::remove_dir_all(folder);
}

#[test]
fn opening_the_folder_deletes_whatever_the_store_no_longer_lists() {
    let folder = folder("sweep");
    fs::create_dir_all(folder.join("someones-folder")).expect("folder");
    for name in [
        "almanac.png",
        "almanac_moon.png",
        "dropped.png",
        "almanac.part-1-0",
        "notes.txt",
    ] {
        fs::write(folder.join(name), picture(10, 10, 0)).expect("file");
    }

    let _pictures = Pictures::open(
        folder.clone(),
        &listed(&["almanac.png", "almanac/moon.png"]),
    );

    let mut left: Vec<String> = fs::read_dir(&folder)
        .expect("folder")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .into_string()
                .expect("name")
        })
        .collect();
    left.sort();
    // A folder is not this code's to delete, whatever it is doing there.
    assert_eq!(left, ["almanac.png", "almanac_moon.png", "someones-folder"]);
    let _ = fs::remove_dir_all(folder);
}

#[test]
fn only_a_few_pictures_are_fetched_at_once_however_many_are_asked_for() {
    let folder = folder("at-once");
    let names: Vec<String> = (0..12).map(|number| format!("shot-{number}.png")).collect();
    let pictures = Pictures::open(folder.clone(), &names.iter().cloned().collect());
    let (going, most) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let (counted, highest) = (Arc::clone(&going), Arc::clone(&most));
    let fetch = move |_: &str, _: u32| {
        let now = counted.fetch_add(1, Ordering::SeqCst) + 1;
        highest.fetch_max(now, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(30));
        counted.fetch_sub(1, Ordering::SeqCst);
        Ok::<_, DeviceError>(picture(10, 10, 0))
    };

    std::thread::scope(|scope| {
        for name in &names {
            let (pictures, fetch) = (&pictures, &fetch);
            scope.spawn(move || assert!(pictures.get(name, fetch).is_some(), "{name}"));
        }
    });

    assert!((1..=AT_ONCE).contains(&most.load(Ordering::SeqCst)));
    assert_eq!(pictures.outward().going, 0);
    let _ = fs::remove_dir_all(folder);
}

#[test]
fn a_fetch_that_panics_gives_its_turn_back() {
    let folder = folder("panic");
    let names = ["a.png", "b.png", "c.png", "d.png"];
    let pictures = Pictures::open(folder.clone(), &listed(&names));
    for name in names {
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            pictures.get(name, &|_, _| -> Result<Vec<u8>, DeviceError> {
                panic!("the transport fell over")
            })
        }));
        assert!(panicked.is_err());
    }
    // More fell over than there are turns, and there is still one to take.
    let (fetch, _) = site(&[("a.png", picture(10, 10, 0))]);
    assert!(pictures.get("a.png", &*fetch).is_some());
    let _ = fs::remove_dir_all(folder);
}
